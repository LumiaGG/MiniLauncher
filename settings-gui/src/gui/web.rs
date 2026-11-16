use super::*;
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::component::{
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    tag::Tag,
    Icon, Sizable, WindowExt,
};

struct EngineForm {
    name: Entity<InputState>,
    keywords: Entity<InputState>,
    url: Entity<InputState>,
    error: Option<String>,
}
impl Render for EngineForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(form_field("名称", Input::new(&self.name)))
            .child(form_field("触发关键词", Input::new(&self.keywords)))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("多个关键词用空格或逗号分隔，例如 gg, google。"),
            )
            .child(form_field("搜索地址", Input::new(&self.url)))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "把搜索词所在位置写成 {query}，例如 https://example.com/search?q={query}。",
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}
fn form_field(title: &'static str, input: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_sm().child(title))
        .child(input)
}

impl Settings {
    fn engine_dialog(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Err(error) = self.collect(cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let engine = page_model::web(&self.draft.value)
            .ok()
            .and_then(|web| web.engines.into_iter().find(|e| Some(&e.id) == id.as_ref()));
        let form = cx.new(|cx| EngineForm {
            name: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(engine.as_ref().map(|e| e.name.clone()).unwrap_or_default())
                    .placeholder("例如 Google")
            }),
            keywords: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(
                        engine
                            .as_ref()
                            .map(|e| e.keywords.join(", "))
                            .unwrap_or_default(),
                    )
                    .placeholder("例如 gg, google")
            }),
            url: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(engine.as_ref().map(|e| e.url.clone()).unwrap_or_default())
                    .placeholder("https://example.com/search?q={query}")
            }),
            error: None,
        });
        self.busy = true;
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let form_submit = form.clone();
            let id = id.clone();
            dialog
                .confirm()
                .title(if id.is_some() {
                    "编辑搜索引擎"
                } else {
                    "添加搜索引擎"
                })
                .width(px(600.))
                .ok_text("完成")
                .cancel_text("取消")
                .child(form.clone())
                .on_ok(move |_, window, cx| {
                    let (name, keywords, url) = {
                        let f = form_submit.read(cx);
                        (
                            f.name.read(cx).value().to_string(),
                            f.keywords.read(cx).value().to_string(),
                            f.url.read(cx).value().to_string(),
                        )
                    };
                    let result = owner
                        .update(cx, |this, cx| {
                            page_model::put_engine(
                                &mut this.draft,
                                id.as_deref(),
                                &name,
                                &keywords,
                                &url,
                            )?;
                            this.rebuild(window, cx);
                            this.queue_save(cx);
                            if !this.flush(window, cx) {
                                return Err(this.error.clone().unwrap_or_default());
                            }
                            Ok::<_, String>(())
                        })
                        .unwrap_or_else(|_| Err("设置窗口已关闭".into()));
                    if let Err(error) = result {
                        form_submit.update(cx, |f, cx| {
                            f.error = Some(error);
                            cx.notify();
                        });
                        false
                    } else {
                        true
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

    fn default_engine(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let result = self.collect(cx).and_then(|_| {
            let mut web = page_model::web(&self.draft.value)?;
            web.default_engine = id.into();
            page_model::commit_web(&mut self.draft, web)
        });
        self.finish_web_change(result, window, cx);
    }

    fn move_engine(&mut self, id: &str, step: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let result = self.collect(cx).and_then(|_| {
            let mut web = page_model::web(&self.draft.value)?;
            let index = web
                .engines
                .iter()
                .position(|e| e.id == id)
                .ok_or("引擎已不存在")?;
            let next =
                (index as i32 + step).clamp(0, web.engines.len().saturating_sub(1) as i32) as usize;
            web.engines.swap(index, next);
            page_model::commit_web(&mut self.draft, web)
        });
        self.finish_web_change(result, window, cx);
    }

    fn finish_web_change(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.rebuild(window, cx);
                self.queue_save(cx);
                self.flush(window, cx);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn delete_engine_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Err(error) = self.collect(cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let Ok(web) = page_model::web(&self.draft.value) else {
            return;
        };
        let Some(engine) = web.engines.iter().find(|e| e.id == id) else {
            return;
        };
        let name = engine.name.clone();
        let description = if web.default_engine == id {
            format!("删除“{name}”后，将自动选择另一个引擎作为默认。确认后立即生效。")
        } else {
            format!("删除“{name}”？确认后立即生效。")
        };
        self.busy = true;
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let id = id.clone();
            dialog
                .confirm()
                .title("删除搜索引擎")
                .description(description.clone())
                .ok_text("删除")
                .cancel_text("取消")
                .on_ok(move |_, window, cx| {
                    owner
                        .update(cx, |this, cx| {
                            let result = page_model::delete_engine(&mut this.draft, &id);
                            let success = result.is_ok();
                            this.finish_web_change(result, window, cx);
                            success
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

    pub(super) fn render_web(&self, cx: &mut Context<Self>) -> AnyElement {
        let Ok(web) = page_model::web(&self.draft.value) else {
            return div()
                .child("网页搜索配置无法读取，请检查配置文件。")
                .into_any_element();
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div().flex().flex_col().gap_1().child("搜索引擎").child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("关键词直接唤出对应引擎；默认引擎用于综合搜索。"),
                        ),
                    )
                    .child(
                        Button::new("add-engine")
                            .icon(Glyph::Plus)
                            .label("添加引擎")
                            .primary()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.engine_dialog(None, window, cx)
                            })),
                    ),
            )
            .children(web.engines.iter().enumerate().map(|(index, engine)| {
                let id = engine.id.clone();
                let edit_id = id.clone();
                let current = web.default_engine == id;
                let count = web.engines.len();
                let owner = cx.entity().downgrade();
                div()
                    .id(SharedString::from(format!("engine-{}", id)))
                    .px_3().py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(Glyph::Globe).text_color(muted))
                            .child(div().flex_1().min_w_0().child(engine.name.clone()))
                            .when(current, |el| el.child(Tag::success().small().child("默认")))
                            .child(
                                Button::new(("edit-engine", index))
                                    .label("编辑")
                                    .small().ghost()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.engine_dialog(Some(edit_id.clone()), window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("engine-menu", index))
                                    .icon(Glyph::Ellipsis)
                                    .small().ghost()
                                    .tooltip("更多操作")
                                    .disabled(self.busy)
                                    .dropdown_menu(move |menu, _, _| {
                                        let default_owner = owner.clone();
                                        let default_id = id.clone();
                                        let up_owner = owner.clone();
                                        let up_id = id.clone();
                                        let down_owner = owner.clone();
                                        let down_id = id.clone();
                                        let delete_owner = owner.clone();
                                        let delete_id = id.clone();
                                        menu.item(
                                            PopupMenuItem::new("设为默认")
                                                .disabled(current)
                                                .on_click(move |_, window, cx| {
                                                    let _ = default_owner.update(cx, |this, cx| {
                                                        this.default_engine(&default_id, window, cx)
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("上移")
                                                .disabled(index == 0)
                                                .on_click(move |_, window, cx| {
                                                    let _ = up_owner.update(cx, |this, cx| {
                                                        this.move_engine(&up_id, -1, window, cx)
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new("下移")
                                                .disabled(index + 1 == count)
                                                .on_click(move |_, window, cx| {
                                                    let _ = down_owner.update(cx, |this, cx| {
                                                        this.move_engine(&down_id, 1, window, cx)
                                                    });
                                                }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("删除")
                                                .disabled(count <= 1)
                                                .on_click(move |_, window, cx| {
                                                    let _ = delete_owner.update(cx, |this, cx| {
                                                        this.delete_engine_dialog(
                                                            delete_id.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            engine
                                .keywords
                                .iter()
                                .map(|word| Tag::secondary().small().child(word.clone())),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .overflow_hidden()
                            .child(engine.url.clone()),
                    )
            }))
            .into_any_element()
    }
}
